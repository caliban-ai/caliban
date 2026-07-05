# Container image

`ghcr.io/caliban-ai/caliban` ships both binaries — `caliban` (the harness) and
`caliband` (the per-repo supervisor) — on `$PATH`. The default entrypoint is
`caliband`.

## Build locally

    docker build -t caliban:dev .

Multi-arch images (`linux/amd64`, `linux/arm64`) are published by the
`release-image` workflow on `v*` tags.

## Run

    # supervise a repo mounted at /work/repo
    docker run --rm -v "$PWD:/work/repo" ghcr.io/caliban-ai/caliban \
      --repo-root /work/repo

`caliband` flags: `--repo-root <path>` (required), `--socket-path <path>`,
`--data-base <path>`.

## Environment

The image sets `HOME=/home/app` and XDG dirs so config/data/socket have a
writable home:

| Var | Purpose | Image default |
|-----|---------|---------------|
| `CALIBAN_ROUTER_CONFIG` | explicit path to `caliban.toml` | unset (falls back to `$XDG_CONFIG_HOME/caliban/caliban.toml`) |
| `XDG_CONFIG_HOME` | config root | `/home/app/.config` |
| `XDG_DATA_HOME` | data root | `/home/app/.local/share` |
| `XDG_RUNTIME_DIR` | runtime/socket root | `/home/app/.run` |
| `CALIBAN_DAEMON_RUNTIME_DIR` | daemon socket dir | `/home/app/.run/caliban` |
| `RUST_LOG` | log filter | unset (`info`) |

Provider credentials (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …) and the model
router config are supplied per deployment — later injected by `caliban-operator`
(see the k8s design spec). This image only guarantees the binaries honour them.

## Sandbox caveat

On Linux, `caliband` isolates subprocess tools with `bubblewrap` (`bwrap`),
which is installed in the image. `bwrap` needs user-namespace support from the
container runtime (it runs with `--unshare-user`).

Sandbox detection currently checks only that `bwrap` is **present and a
supported version** — it does not yet probe whether the runtime actually permits
user namespaces. The fail-open-to-unsandboxed path fires when `bwrap` is missing
or too old, **not** when it is installed but userns is denied. So on a runtime
that ships `bwrap` yet forbids user namespaces (e.g. stock Docker under the
default seccomp profile, or `kernel.unprivileged_userns_clone=0`), sandboxed
tool calls **fail** rather than degrading to unsandboxed.

To run there today, grant the container user-namespace access (an appropriate
`securityContext` / `--security-opt seccomp=unconfined`) or run without the
sandbox. A real userns probe with graceful fallback, and arbitrary-UID
(`runAsUser`) hardening of the baked `HOME`/`XDG_RUNTIME_DIR`, are tracked in
#345. Pod-level isolation (gVisor/Kata via agent-sandbox) is the k8s-era
replacement (design spec §6).
