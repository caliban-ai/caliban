# OS-level sandbox — Design

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0032-os-sandbox.md`
**Depends on:** `caliban-tools-builtin::BashTool` (the shim wraps its
child-process plumbing), `caliban-core` (settings hierarchy — the
sandbox config lives under the broader settings work).

## Goal

Wrap Bash invocations (and, by opt-in, other subprocess-spawning tools)
in an OS-level sandbox that restricts filesystem reach, network egress,
and macOS Mach lookups. Two backends, one config surface: macOS uses
Apple's Seatbelt (`sandbox-exec` with a generated `.sb` profile); Linux
uses bubblewrap (`bwrap` with `--bind` / `--ro-bind` / `--share-net`
flags). WSL inherits the Linux path. With the sandbox enabled, the
operator can drop the per-command Ask prompt for Bash entirely — the
sandbox itself is the protection. Windows native is **not supported in
v1** (deferred — needs Job Objects + AppContainer + a third policy
generator).

## Non-goals

- **Windows native sandboxing.** v1 documents the gap and refuses to
  enable; v2 will add a Job Object + AppContainer backend.
- **Per-call dynamic policies.** The Seatbelt profile and `bwrap` argv
  are computed once per session from settings; we do not synthesize a
  new policy per Bash call.
- **Sandboxing the caliban binary itself.** Only child processes are
  sandboxed; the agent process retains full permissions (it has to —
  it speaks to the provider, manages MCP, etc.).
- **Network egress proxying as a feature.** We support
  `network.http_proxy_port` / `network.socks_proxy_port` so a separately-run
  proxy can be the egress chokepoint, but we don't ship the proxy.
- **Replacing the permission system.** Sandboxing complements permission
  rules; it doesn't replace them. The `Read(./.env)` deny rule still
  applies regardless of sandbox state.
- **Container backends (Docker / Podman / Firejail / Landlock-only).**
  Out of scope for v1. Operators who want containers can run caliban
  inside one and set `sandbox.enabled = false`.

## Architecture

```
caliban-tools-builtin::BashTool::invoke(input)
   │
   ▼
SandboxedShim::wrap_command(cmd, sandbox_cfg)
   │
   ├─ if sandbox disabled → return cmd unchanged
   ├─ if command in allow_unsandboxed_commands → return cmd unchanged
   └─ if enabled and available:
        ├─ macos:    sandbox-exec -f <generated.sb> -- <cmd>  (or -p inline)
        ├─ linux:    bwrap [bind flags] [share-net flag] [--die-with-parent] -- <cmd>
        └─ wsl:      same as linux
   │
   ▼
spawn child  (existing tokio::process::Command path)

policy generation (per session):
  SandboxConfig (from settings)
    │
    ├─ MacOsSeatbeltPolicy::render()  →  .sb profile text
    └─ BwrapArgs::render()            →  Vec<OsString> for bwrap
```

The sandbox is a *shim layer* over the existing `BashTool` child-process
machinery, not a rewrite. `BashTool` builds its `tokio::process::Command`
exactly as today; `SandboxedShim::wrap_command` either returns it
unchanged or wraps it in a new `Command` whose program is
`sandbox-exec` / `bwrap` and whose args carry the original command plus
the sandbox flags. PID-group cleanup (the existing `kill_process_tree`
logic) still works — `sandbox-exec` and `bwrap` propagate signals to
their child.

## Crate structure (delta)

```
crates/caliban-sandbox/                # NEW
├── Cargo.toml
└── src/
    ├── lib.rs                  # SandboxedShim + re-exports
    ├── config.rs               # SandboxConfig, FilesystemAcl, NetworkAcl
    ├── shim.rs                 # SandboxedShim::wrap_command
    ├── seatbelt.rs             # MacOsSeatbeltPolicy + .sb text gen
    ├── bwrap.rs                # BwrapArgs + argv gen
    ├── detect.rs               # locate sandbox-exec / bwrap; capability probe
    ├── nested.rs               # enable_weaker_nested_sandbox handling
    └── error.rs                # SandboxError

crates/caliban-tools-builtin/src/bash.rs
  + BashTool::with_sandbox(SandboxedShim) constructor
  + invoke() calls shim.wrap_command(cmd, &input) before spawn

caliban/src/main.rs
  + parse sandbox.* settings; build SandboxedShim; pass to BashTool
```

Dependencies (new): none beyond workspace. Sandbox is pure
process-construction; no FFI.

## Settings keys

```toml
[sandbox]
# Master switch. When false, every wrap_command is a no-op (matches
# Claude Code's `sandbox.enabled`).
enabled = false

# When true and the configured sandbox backend isn't available on $PATH,
# caliban refuses to start instead of running unsandboxed.
fail_if_unavailable = false

# When true and sandbox.enabled is true, Bash skips the Ask permission
# prompt (sandbox is the protection). Defaults to false so operators
# opt in deliberately.
auto_allow_bash_if_sandboxed = false

# Commands that should never be sandboxed even when sandbox is on.
# Matched against the *first token* of the command (after splitting on
# whitespace). Useful for tooling that breaks under bwrap (e.g. git
# hooks that need network without going through the proxy).
allow_unsandboxed_commands = ["git", "gh"]

# Allow nesting (running caliban inside a dev container / VM that
# already restricts the environment). When true, we relax checks that
# would otherwise refuse to start under an existing namespace.
enable_weaker_nested_sandbox = false

# Path overrides for non-default install locations.
bwrap_path = "/usr/bin/bwrap"               # default: search $PATH
sandbox_exec_path = "/usr/bin/sandbox-exec" # default: /usr/bin/sandbox-exec (system)

[sandbox.filesystem]
allow_write = [
  "${WORKSPACE}",
  "${HOME}/.cache",
  "/tmp",
]
deny_write = [
  "${WORKSPACE}/.env",
  "${WORKSPACE}/.git/hooks",
]
allow_read = [
  "/",                  # default-allow all reads; trim if you want
]
deny_read = [
  "${HOME}/.ssh",
  "${HOME}/.aws",
  "${HOME}/.gnupg",
]

[sandbox.network]
# Hostnames the sandboxed process may resolve and reach. Wildcards
# allowed (`*.github.com`). Empty list = no egress.
allowed_domains = ["github.com", "*.github.com", "registry.npmjs.org"]
denied_domains  = []                  # explicit blacklist within allow
http_proxy_port  = 0                  # 0 = none; non-zero = route HTTP via 127.0.0.1:<port>
socks_proxy_port = 0                  # 0 = none

# Unix socket access (e.g. Docker daemon at /var/run/docker.sock).
allow_unix_sockets = false
# Allow binding local listening ports (servers under test).
allow_local_binding = false

# macOS-only: Mach service lookups. Each entry is a service name like
# "com.apple.SecurityServer" or "*.audio". Empty = none allowed.
allow_mach_lookup = ["com.apple.distributed_notifications.2"]
```

`${WORKSPACE}`, `${HOME}`, `${XDG_CONFIG_HOME}`, `${XDG_DATA_HOME}`,
`${XDG_CACHE_HOME}` are expanded at session start. Paths are normalized
(resolved symlinks, made absolute). Globs aren't supported in
filesystem ACLs — operators add explicit roots.

## Backend: macOS Seatbelt

The Seatbelt profile is a generated `.sb` (TinyScheme dialect) file
written to `$XDG_RUNTIME_DIR/caliban/sandbox/<session-id>.sb` (or
`/tmp/caliban-sandbox-<session-id>.sb` if `XDG_RUNTIME_DIR` is unset)
with mode `0600`. The profile is recomputed at session start, not per
call.

```scheme
(version 1)
(deny default)

;; Allow basic process operations the agent's child needs.
(allow process-fork)
(allow process-exec)
(allow signal (target self))
(allow sysctl-read)

;; Reads — allow_read minus deny_read.
(allow file-read*
  (subpath "/")
  (subpath "${HOME}")
)
(deny  file-read*
  (subpath "${HOME}/.ssh")
  (subpath "${HOME}/.aws")
  (subpath "${HOME}/.gnupg")
)

;; Writes — allow_write minus deny_write.
(allow file-write*
  (subpath "${WORKSPACE}")
  (subpath "${HOME}/.cache")
  (subpath "/tmp")
)
(deny  file-write*
  (subpath "${WORKSPACE}/.env")
  (subpath "${WORKSPACE}/.git/hooks")
)

;; Mach lookups — allow_mach_lookup.
(allow mach-lookup (global-name "com.apple.distributed_notifications.2"))

;; Network — allowed_domains compiled to remote-host literals where
;; possible; otherwise full network-outbound restricted to proxy port.
(allow network-outbound
  (remote tcp "github.com:443")
  (remote tcp "*.github.com:443")
  (remote tcp "registry.npmjs.org:443")
)
(allow network-bind  (local ip "*:0"))    ; only when allow_local_binding = true
```

When `network.http_proxy_port` is set, the profile permits
`(allow network-outbound (remote tcp "127.0.0.1:<port>"))` only; the
sandbox blocks direct egress and the proxy enforces domain rules.

`MacOsSeatbeltPolicy::render() -> String` returns the profile text;
`SandboxedShim` writes it to disk and passes the path with `-f`.
`sandbox-exec` is `/usr/bin/sandbox-exec` on every supported macOS;
overridable via `sandbox_exec_path`.

### Caveats

`sandbox-exec` is deprecated by Apple but still ships and works on
every macOS through current. The profile dialect is undocumented but
stable; we mirror the dialect used by Chrome's renderer sandbox and
Claude Code's profile (reverse-engineered from public traces). If
Apple removes `sandbox-exec`, we revisit.

## Backend: Linux bubblewrap

```
bwrap                                           \
  --die-with-parent                             \
  --new-session                                 \
  --unshare-user                                \
  --unshare-pid                                 \
  --unshare-ipc                                 \
  --unshare-cgroup-try                          \
  --proc /proc                                  \
  --dev  /dev                                   \
  --tmpfs /tmp                                  \
  --setenv HOME "$HOME"                         \
  --ro-bind /usr /usr                           \
  --ro-bind /etc /etc                           \
  --ro-bind /bin /bin                           \
  --ro-bind /lib /lib                           \
  --ro-bind /lib64 /lib64                       \
  --ro-bind "$HOME/.cargo" "$HOME/.cargo"       \
  --bind   "${WORKSPACE}"   "${WORKSPACE}"      \
  --bind   "${HOME}/.cache" "${HOME}/.cache"    \
  --tmpfs  "${HOME}/.ssh"                       \
  --tmpfs  "${HOME}/.aws"                       \
  --tmpfs  "${HOME}/.gnupg"                     \
  --unshare-net                                 \  # when allowed_domains is empty
  --                                            \
  /bin/sh -c "<command>"
```

Detailed mapping:

| Setting | Bwrap flag |
|---|---|
| `allow_read[*]` | `--ro-bind <path> <path>` (one per entry) |
| `allow_write[*]` | `--bind <path> <path>` |
| `deny_read[*]` / `deny_write[*]` | `--tmpfs <path>` (masks the real path with an empty tmpfs) |
| `allowed_domains[*] == []` | `--unshare-net` |
| `allowed_domains[*]` non-empty | keep network namespace; rely on the proxy or DNS-level filtering (see proxy section) |
| `http_proxy_port = N` | `--unshare-net` + `--bind /var/run/socat-<sessid>.sock /var/run/socat.sock` (the proxy bridge); set `HTTP_PROXY=http://127.0.0.1:<N>` in env |
| `allow_unix_sockets = true` | omit the `--unshare-ipc` line; bind specific sockets via `--bind <socket> <socket>` |
| `allow_local_binding = true` | retain network namespace; no `--unshare-net` |
| `enable_weaker_nested_sandbox = true` | drop `--unshare-user` (we're already in a user namespace) |

`bwrap` path is searched in `$PATH`; overridable via `bwrap_path`. v1
requires `bwrap >= 0.5` (we check at startup via `bwrap --version`).

### Network egress on Linux

When `allowed_domains` is set without a proxy port, we don't have an
in-kernel way to enforce hostname-level rules under `bwrap` alone.
v1 documents this honestly:

- Empty `allowed_domains`: `--unshare-net` — no egress at all.
- `http_proxy_port` set: `--unshare-net` plus a binding to a
  user-supplied proxy socket. The proxy enforces hostnames.
- Both unset, `allowed_domains` non-empty: log a warning at startup;
  the sandbox is *less restrictive* than the operator probably
  intended. Add a follow-up to ship an in-tree minimal HTTP proxy
  (v1.1) that consumes `allowed_domains`.

## Backend selection and detection

```rust
// caliban-sandbox/src/detect.rs

pub enum Backend {
    Seatbelt { path: PathBuf },
    Bwrap    { path: PathBuf, version: SemverString },
    Unavailable,
}

pub fn detect(config: &SandboxConfig) -> Result<Backend, SandboxError>;
```

`detect`:

- On macOS, checks `sandbox_exec_path` (or `/usr/bin/sandbox-exec`); if
  missing and `fail_if_unavailable: true`, errors; else falls back to
  `Unavailable`.
- On Linux/WSL, runs `bwrap --version`, parses the version; requires
  `>= 0.5`; if missing/old and `fail_if_unavailable: true`, errors.
- On Windows native (and `cfg!(windows)`), always returns
  `Unavailable` with a clear error if `sandbox.enabled: true`.

`SandboxedShim::new(config) -> Result<Self, SandboxError>` runs
`detect` once at construction and stores the backend.

## `BashTool` integration

```rust
// caliban-tools-builtin/src/bash.rs (sketch of the change)

pub struct BashTool {
    root: Arc<WorkspaceRoot>,
    sandbox: Option<Arc<SandboxedShim>>,   // None == no sandbox
    schema: OnceLock<Value>,
}

impl BashTool {
    pub fn with_sandbox(root: WorkspaceRoot, sandbox: Option<Arc<SandboxedShim>>) -> Self;

    /// In `invoke`, after building the base `tokio::process::Command`:
    fn maybe_wrap(&self, cmd: tokio::process::Command, input: &BashInput) -> tokio::process::Command {
        let Some(shim) = self.sandbox.as_ref() else { return cmd };
        shim.wrap_command(cmd, &input.command)
    }
}
```

`wrap_command(cmd, command_str)`:

1. Look at the first token of `command_str`. If it matches any entry in
   `allow_unsandboxed_commands`, return `cmd` unchanged.
2. Otherwise, build a new `Command` whose program is the sandbox binary
   (Seatbelt or bwrap), whose first args are the policy flags, and
   whose tail args invoke the original program. Preserve the original
   command's `current_dir`, `env`, `kill_on_drop`, and `process_group`
   settings.

`process_group(0)` continues to work because Seatbelt/bwrap inherit
their child's group. The existing `kill_process_tree` logic targets
the wrapper's PID, which on signal propagates SIGKILL down to the
agent's command — same outcome as today.

## `auto_allow_bash_if_sandboxed`

When `sandbox.enabled: true` and `auto_allow_bash_if_sandboxed: true`:

- The permission classifier short-circuits `Bash(*)` to `allow` *before*
  the Ask modal would fire. The rule grammar is not modified; the
  short-circuit sits in the permission evaluation pipeline alongside
  the existing mode-based bypass (plan mode, etc.).
- The TUI status line shows a `sandbox: on` chip.
- Commands matching `allow_unsandboxed_commands` are *not*
  auto-allowed — they still go through the normal ask/allow rules
  (because they're running unsandboxed).
- `Bash(rm -rf /)` is still bounded by what the sandbox actually
  permits — `/` is read-only outside the listed `allow_write` paths.

## Error handling

```rust
pub enum SandboxError {
    BackendUnavailable { backend: &'static str, looked_at: PathBuf },
    BackendTooOld      { backend: &'static str, found: String, need: String },
    PolicyWrite        { source: std::io::Error, path: PathBuf },
    InvalidConfig      { reason: String },
    UnsupportedPlatform { os: &'static str },
}
```

`BackendUnavailable` + `fail_if_unavailable: true` is fatal at startup.
`InvalidConfig` (e.g. `bwrap_path` points at a non-executable) is
fatal. `PolicyWrite` is fatal at first call but logged + retried on
subsequent calls.

## Public API sketches

```rust
// caliban-sandbox/src/lib.rs

pub use config::{SandboxConfig, FilesystemAcl, NetworkAcl};
pub use shim::SandboxedShim;
pub use detect::Backend;
pub use error::SandboxError;

// caliban-sandbox/src/shim.rs

pub struct SandboxedShim {
    backend: Backend,
    config: SandboxConfig,
    seatbelt_profile_path: Option<PathBuf>,  // None on Linux
}

impl SandboxedShim {
    pub fn new(config: SandboxConfig) -> Result<Self, SandboxError>;
    pub fn backend(&self) -> &Backend;
    pub fn wrap_command(
        &self,
        cmd: tokio::process::Command,
        command_str: &str,
    ) -> tokio::process::Command;
}
```

## Tests

1. **`config_parses_minimal_defaults`** — empty `[sandbox]` block parses
   to `enabled: false` with no errors.
2. **`config_expands_workspace_and_home_vars`** — `${WORKSPACE}` and
   `${HOME}` expand to the test fixtures' values.
3. **`detect_seatbelt_present_on_macos`** *(cfg `target_os = "macos"`)*
   — `detect` returns `Backend::Seatbelt`.
4. **`detect_bwrap_version_too_old_errors_when_fail_if_unavailable`**
   *(cfg `target_os = "linux"`)* — fake `bwrap` script reports 0.3,
   `fail_if_unavailable: true`, `detect` returns `BackendTooOld`.
5. **`detect_windows_returns_unsupported`** *(cfg `target_os =
   "windows"`)* — always `UnsupportedPlatform { os: "windows" }`.
6. **`shim_disabled_returns_cmd_unchanged`** — `enabled: false`,
   `wrap_command(cmd, "ls")` returns the same program/args.
7. **`shim_allow_unsandboxed_commands_bypasses_wrap`** — `allow_unsandboxed_commands = ["git"]`, `wrap_command(cmd, "git status")` returns unchanged.
8. **`seatbelt_policy_renders_allow_write_subpaths`** — generated `.sb`
   text contains `(subpath "${WORKSPACE}")` after expansion.
9. **`seatbelt_policy_denies_writes_to_dotenv`** — `deny_write` paths
   appear in a `(deny file-write* …)` block.
10. **`seatbelt_policy_no_network_when_allowed_domains_empty`** —
    rendered profile has *no* `network-outbound` allow rule.
11. **`seatbelt_policy_emits_mach_lookup_entries`** —
    `allow_mach_lookup = ["com.foo"]` appears as
    `(allow mach-lookup (global-name "com.foo"))`.
12. **`bwrap_args_unshare_net_when_no_domains`** — generated argv
    contains `--unshare-net`.
13. **`bwrap_args_ro_bind_for_allow_read`** — `allow_read = ["/etc"]`
    → argv contains `--ro-bind /etc /etc`.
14. **`bwrap_args_bind_for_allow_write`** — `allow_write = ["/work"]`
    → argv contains `--bind /work /work`.
15. **`bwrap_args_tmpfs_masks_deny_read`** — `deny_read =
    ["/home/u/.ssh"]` → argv contains `--tmpfs /home/u/.ssh`.
16. **`bwrap_args_drop_unshare_user_when_nested`** —
    `enable_weaker_nested_sandbox: true` → `--unshare-user` not present.
17. **`bash_tool_runs_command_through_seatbelt_on_macos`** *(cfg
    `target_os = "macos"`)* — end-to-end: with sandbox enabled and
    `/tmp` writable, `Bash("echo hi > /tmp/x && cat /tmp/x")` returns
    `hi`.
18. **`bash_tool_denies_write_outside_allow_write`** — with `/tmp` *not*
    in `allow_write`, the same command fails with a permission error.
19. **`auto_allow_bash_if_sandboxed_skips_ask_modal`** — integration:
    Bash invocation under sandbox + flag produces no Ask event.
20. **`fail_if_unavailable_blocks_startup_when_bwrap_missing`** *(cfg
    `target_os = "linux"`)* — set `bwrap_path` to a nonexistent file,
    `fail_if_unavailable: true`, `SandboxedShim::new` errors.

## Risks

- **macOS Seatbelt is deprecated.** Apple has signaled removal for
  years without actually doing it. Mitigation: pin the dialect we use;
  document the risk; if removed in a future macOS, ship an Endpoint
  Security Framework-based backend.
- **bwrap version skew.** Distros ship a range of versions. v1
  requires `>= 0.5`; older versions miss `--die-with-parent`.
  Mitigation: version check at startup; clear error.
- **Network egress is hostname-based, sandboxes are CIDR-based.** Both
  backends struggle to enforce per-hostname rules without a proxy.
  Mitigation: document the proxy-port pattern; ship an in-tree minimal
  proxy in v1.1.
- **Per-tool overrides for non-Bash subprocess tools.** WebFetch
  doesn't spawn a subprocess (it's a Rust HTTP call), so the sandbox
  doesn't apply. Mitigation: document explicitly; WebFetch has its own
  domain allowlist; permission rules still apply.
- **`auto_allow_bash_if_sandboxed` is foot-gun-shaped.** Combined with
  a too-permissive sandbox, it removes the operator from the loop
  entirely. Mitigation: default to `false`; require the operator to
  set both `enabled` and `auto_allow_...`; surface the chip
  prominently in the TUI status line.
- **Process-group cleanup under wrappers.** `bwrap` is the leader of
  the new PID namespace; killing it should reap everything inside.
  Verified by test #18-equivalent (kill the wrapper PID, child shells
  inside the namespace die). If a regression appears, fall back to the
  existing `kill_process_tree` logic on the wrapper PID.
- **Nested sandboxing in dev containers.** A container already restricts
  the namespace; running bwrap inside it may fail without
  `--unshare-user` skipped. `enable_weaker_nested_sandbox: true`
  handles this. Detection of "already in a sandbox" is best-effort
  (`/proc/self/uid_map` heuristic) — operator-driven for v1.

## Acceptance criteria

- `cargo build --workspace`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo fmt --all -- --check` clean.
- ≥18 tests passing in `caliban-sandbox`, plus 2 integration tests in
  `caliban-tools-builtin/tests/` (one per backend, behind
  `#[cfg(target_os = …)]`).
- `BashTool` wraps via `SandboxedShim` when configured; falls back
  cleanly when not.
- `caliban --debug` shows the resolved backend and policy path at
  startup when sandbox is enabled.
- Matrix A "OS-level sandbox" row 🔴 → ✅.
- README documents the macOS / Linux setup, the `bwrap` install
  requirement on common distros, the proxy-port pattern, and the
  Windows-deferred note.
- ADR 0032 in `accepted` status.
