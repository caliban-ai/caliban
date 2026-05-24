# caliban-sandbox

OS-level sandbox layer for caliban's subprocess tools. Implements
[ADR 0032](../../adrs/0032-os-sandbox.md) /
[design](../../docs/superpowers/specs/2026-05-24-os-sandbox-design.md).

## Backends

| Host | Backend | Binary |
|------|---------|--------|
| macOS | Apple Seatbelt | `sandbox-exec` (`/usr/bin/sandbox-exec`) |
| Linux | bubblewrap | `bwrap` (search `$PATH`, requires `>= 0.5`) |
| WSL | bubblewrap | same as Linux |
| Windows native | (not supported in v1) | no-op + warning |

## Quick start

Add to your settings:

```toml
[sandbox]
enabled = true
fail_if_unavailable = true
auto_allow_bash_if_sandboxed = true
allow_unsandboxed_commands = ["git", "gh"]

[sandbox.filesystem]
allow_write = ["/workspace", "/tmp"]
deny_read  = ["/Users/u/.ssh", "/Users/u/.aws"]

[sandbox.network]
allowed_domains = ["github.com", "*.github.com"]
```

## Installing `bwrap` (Linux)

| Distro | Command |
|--------|---------|
| Debian / Ubuntu | `apt install bubblewrap` |
| Fedora | `dnf install bubblewrap` |
| Arch | `pacman -S bubblewrap` |
| Alpine | `apk add bubblewrap` |

`bwrap >= 0.5` is required (`--die-with-parent`). Distros older than
Debian Buster / Ubuntu 18.04 will need a backport.

## Proxy pattern for per-hostname egress

Neither Seatbelt nor `bwrap` enforces per-hostname egress rules on its
own. To get domain-level control, run a separate HTTP proxy on the
local loopback and point the sandbox at it:

```toml
[sandbox.network]
allowed_domains  = []       # no direct egress
http_proxy_port  = 8888     # proxy listens here
```

The sandbox will then permit only `127.0.0.1:8888`; the proxy enforces
domain rules.

## Running the live-backend tests

Most tests run on any host. End-to-end tests that actually exec
`sandbox-exec` / `bwrap` are marked `#[ignore]`:

```sh
# All non-ignored tests (default):
cargo test -p caliban-sandbox

# Live-backend tests (requires sandbox-exec on macOS or bwrap on Linux):
cargo test -p caliban-sandbox -- --ignored
```

## Windows note

Windows native sandboxing requires Job Objects + AppContainer + a third
policy generator and is deferred to v2. WSL inherits the Linux path —
install `bubblewrap` inside the WSL distribution.
