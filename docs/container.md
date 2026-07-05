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

Sandbox detection checks that `bwrap` is present and a supported version, **and
probes whether the runtime actually permits an unprivileged user namespace**. On
a runtime that ships `bwrap` yet forbids userns (e.g. stock Docker under the
default seccomp profile, or `kernel.unprivileged_userns_clone=0`), the probe
fails and caliban logs a warning and runs **unsandboxed** rather than failing
every tool call. Set the sandbox policy's `fail_if_unavailable` if you would
rather hard-fail than run without isolation.

To keep the sandbox on such a runtime, grant the container user-namespace access
(an appropriate `securityContext` / `--security-opt seccomp=unconfined`).
Pod-level isolation (gVisor/Kata via agent-sandbox) is the k8s-era replacement
(design spec §6).

> **Arbitrary-UID note:** the image bakes `HOME=/home/app` (uid 10001) and a
> `0755` `XDG_RUNTIME_DIR`, so an `runAsUser` override that isn't 10001 can't
> write them. Run as uid 10001 or supply a `securityContext` until the image is
> hardened (tracked in #345).
