# The OS Sandbox

Caliban can wrap every subprocess spawned by the `Bash` tool in an OS-level sandbox that restricts what the child process may do — independent of permission rules. Where permission rules decide *whether* a command runs, the sandbox controls *what it can access* once it does.

The sandbox is implemented by the `caliban-sandbox` crate (ADR 0032). It is **disabled by default** and must be explicitly enabled in settings.

## Platform support

| Platform | Backend | Status |
|----------|---------|--------|
| macOS | Apple Seatbelt (`sandbox-exec`) | Supported |
| Linux / WSL | bubblewrap (`bwrap >= 0.5`) | Supported |
| Windows native | — | Not supported in v1; use WSL for the bubblewrap backend |

```admonish warning title="Seatbelt deprecation"
Apple has deprecated the `sandbox-exec` / Seatbelt API. It still ships in all
current macOS releases, but caliban's macOS backend will need to move to the
Endpoint Security Framework if Apple removes `sandbox-exec` in a future OS
version. There is no announced removal date.
```

## Enabling the sandbox

Add a `[sandbox]` block to your project or user `settings.toml`:

```toml
[sandbox]
enabled = true
fail_if_unavailable = true   # refuse to start if bwrap/sandbox-exec is missing
```

With `fail_if_unavailable = false` (the default), caliban falls back to running unsandboxed if the backend binary is absent or too old, and logs a warning.

## What the sandbox restricts

The sandbox limits three classes of access for spawned subprocesses:

### Filesystem

| Key | Effect |
|-----|--------|
| `filesystem.allow_read` | Paths the subprocess may read |
| `filesystem.deny_read` | Paths hidden from reads (shadows an `allow_read` entry) |
| `filesystem.allow_write` | Paths the subprocess may write |
| `filesystem.deny_write` | Paths write-denied within an `allow_write` root |

On Linux, denied paths are masked with `--tmpfs` (an empty in-memory directory shadows the real one). On macOS, Seatbelt uses `(deny file-write* (subpath …))` rules. Glob patterns are **not** supported in filesystem ACLs — add explicit path roots.

The environment variables `${WORKSPACE}`, `${HOME}`, and the XDG vars are expanded when the sandbox is initialized.

### Network

Per-hostname egress is not reliably enforceable by either backend alone. The supported patterns are:

- **Block all egress** — leave `network.allowed_domains` empty. Uses `--unshare-net` on Linux and omits all `network-outbound` allow rules on macOS.
- **Proxy-filtered egress** — set `network.http_proxy_port` to route subprocess HTTP through an operator-run proxy at `127.0.0.1:<port>`. The proxy enforces domain rules; the sandbox only allows the loopback port.

```admonish note title="Per-hostname rules on Linux"
If you set `allowed_domains` to a non-empty list on Linux without also
configuring `http_proxy_port`, caliban logs a warning: the Linux bubblewrap
backend cannot enforce per-hostname rules without a proxy layer.
macOS Seatbelt supports literal `(remote tcp "host:port")` rules and is
correspondingly stricter.
```

### Other network settings

```toml
[sandbox.network]
allow_unix_sockets = false     # Docker daemon socket, etc.
allow_local_binding = false    # bind() on local ports
allow_mach_lookup = []         # macOS-only: Mach service names
```

## Full configuration example

```toml
[sandbox]
enabled = true
fail_if_unavailable = true
auto_allow_bash_if_sandboxed = true
allow_unsandboxed_commands = ["git", "gh"]
enable_weaker_nested_sandbox = false

[sandbox.filesystem]
allow_read  = ["${WORKSPACE}", "/etc", "/usr"]
deny_read   = ["${HOME}/.ssh"]
allow_write = ["${WORKSPACE}"]
deny_write  = ["${WORKSPACE}/.git/hooks"]

[sandbox.network]
http_proxy_port = 8888
allow_unix_sockets = false
allow_local_binding = false
```

## Key settings

**`auto_allow_bash_if_sandboxed`** — When both `enabled` and this flag are `true`, the permission classifier auto-allows all `Bash(*)` calls without showing a prompt. The sandbox is the protection; the Ask modal becomes redundant. Defaults to `false`. Note: commands listed in `allow_unsandboxed_commands` are *not* auto-allowed — they run outside the sandbox and still go through normal permission rules.

**`allow_unsandboxed_commands`** — A glob list matched against the first token of each command (or the full command string when the pattern contains a space). Matching commands bypass the sandbox entirely. Use this for tools that genuinely need unrestricted access — for example, `git` or `gh`.

**`enable_weaker_nested_sandbox`** — For dev containers or VMs that are already inside a user namespace: drops the `--unshare-user` flag on Linux (which would otherwise fail). This is a no-op on macOS.

**`bwrap_path` / `sandbox_exec_path`** — Override the path to the sandbox binary if it is not at the default location (`$PATH` for `bwrap`; `/usr/bin/sandbox-exec` for macOS).

## How it works

`SandboxedShim::wrap_command` intercepts the `tokio::process::Command` built by `BashTool` before it is spawned. If the sandbox is active and the command is not on the bypass list, it rewrites the command so that:

- On macOS: `sandbox-exec -f <profile.sb> <original command>`
- On Linux: `bwrap [bind/ro-bind/tmpfs flags] <original command>`

The rest of the Bash tool — stdout/stderr capture, PID-group cleanup, timeouts, cancellation — is unchanged. The sandbox is a shim layer, not a fork.

Detection runs at startup. `bwrap` version >= 0.5 is required on Linux (the `--die-with-parent` flag arrived in 0.5).
