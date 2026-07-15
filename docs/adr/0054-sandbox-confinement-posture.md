# ADR 0054 · Sandbox confinement posture: reads open, egress closed

- **Status:** accepted
- **Date:** 2026-07-12
- **Source:** [`docs/superpowers/specs/2026-07-12-sandbox-egress-confinement-design.md`](../superpowers/specs/2026-07-12-sandbox-egress-confinement-design.md)

## Context

caliban's OS sandbox (ADR 0032) is an **opt-in write fence**. It engages only
under `--workspace` / `--restrict-paths` — a plain interactive run has no
sandbox at all — and it wraps **only Bash tool commands**. MCP servers and
sub-agent workers spawn on separate paths and never pass through the shim.

Until this decision, the fence conceded three things at once:

- **Reads:** `allow_read: ["/"]` — the whole host, including `~/.ssh`,
  `~/.aws/credentials`, `~/.config/gh/hosts.yml`, `~/.netrc`, caliban's own
  config, and the MCP OAuth token store (`mcp-tokens.json`).
- **Egress:** `allow_all_outbound: true` — deliberately, so `git fetch`,
  `cargo`, and `curl` kept working.
- **Environment:** the child inherited caliban's full environment, including
  `ANTHROPIC_API_KEY` and `CALIBAN_*` tokens.

The sandboxed child runs as **the same uid** as the user, so file permissions
provide nothing: mode `0600` on the token store is irrelevant to a process
running as its owner. Reads plus egress means any Bash command the model runs
can ship the user's credentials to an arbitrary host.

**Threat model** (never previously written down, and the root of the whole
2026-07 sandbox ticket cluster — #402, #403, #404, #405, #406, #407): the
adversary is **not the user**. It is **untrusted content** — a repo file, a
fetched web page, an MCP tool result — that steers the agent into running a
command it should not run. The sandbox exists to bound the blast radius of a
prompt-injection-driven command.

**What the field does** (surveyed 2026-07: Codex CLI, Claude Code, Cursor,
Gemini CLI, Goose, aider, OpenHands, Devin):

- **Nobody read-jails.** Unanimous. Claude Code documents plainly that its
  default "still allows reading credential files such as `~/.aws/credentials`
  and `~/.ssh/`". Codex cannot restrict reads at all.
- **The serious harnesses close egress.** Codex CLI blocks network by default in
  `workspace-write` mode. Claude Code blocks it behind an allowlist plus an
  external proxy, with no domains pre-allowed. Gemini CLI, Goose, and aider
  leave egress open — but Goose and aider ship no sandbox at all, so they
  promise nothing.

Anthropic's published rationale is the load-bearing argument: filesystem and
network isolation are meaningful only **together** — *"Without network
isolation, a compromised agent could exfiltrate sensitive files like SSH keys."*
Open reads are defensible **only because** egress is closed.

caliban was the only surveyed harness that shipped a sandbox while conceding
reads, egress, and environment simultaneously.

### Alternatives weighed

- **Read confinement (allowlist reads).** Rejected. Not industry practice, high
  breakage (commands legitimately need `~/.gitconfig`, `~/.cargo`, `~/.rustup`,
  toolchain caches), and low marginal value once egress is shut.
- **Env scrubbing alone** (the original framing of #405). Rejected as a
  standalone fix: with reads and egress both open, a command that wants
  credentials never has to touch the environment — it reads the file and
  `curl`s it out. Scrubbing the env makes the exfiltration one `cat` longer.
  It becomes honest defense-in-depth *after* egress is closed (#405).
- **Document the write-only fence and change nothing.** Rejected: a user who
  types `--workspace` reasonably reads "fence" as protection, and "we
  documented it" is a thin answer for a harness that runs commands driven by
  untrusted content.

## Decision

**We will keep filesystem reads open and close network egress.**

Under `--workspace` (or `--restrict-paths`), a sandboxed Bash command may read
the disk but has no route off the machine. Writes remain confined to the
workspace plus temp dirs.

- `workspace_fence_policy()` sets `allow_all_outbound: false` and
  `allow_local_binding: true`.
- **Loopback stays up** so localhost test and dev servers keep working. This is
  free on Linux (`--unshare-net` yields an isolated netns with `lo` up) and
  requires an explicit rule on macOS, where Seatbelt's `(deny default)` would
  otherwise deny loopback along with egress.
- **Escape hatch:** `--sandbox-network=allow`, or `sandbox.network = "allow"` in
  `settings.json`. CLI beats settings; settings beat the default (deny).
- A sandboxed command that fails while egress is denied gets an explicit note
  saying so and naming the opt-out.

**The guarantee we now make:** writes are confined; the network is blocked.

**What we explicitly do NOT claim:** the sandbox is **not a read jail** and
**not a secrets boundary** against an attacker who has egress by another route.
`~/.ssh` and `~/.aws/credentials` remain readable. That is safe only while
egress is shut — which is precisely why egress must stay shut, and why
re-opening it with `--sandbox-network=allow` restores the exfiltration path.

## Consequences

- **Breaking.** `--workspace` now also means "no network". Sandboxed `git
  fetch`, `cargo` against crates.io, `npm install`, `gh`, and `curl` fail unless
  the user opts out. Shipped in 0.7.0 with a BREAKING changelog entry. This is
  the same tradeoff Codex made, and the same complaint Codex fields (`npm
  install` hanging is its most-reported gotcha) — hence the explicit error note.
- **The sandbox becomes user-configurable for the first time.** The `[sandbox]`
  TOML table described in `caliban-sandbox/src/config.rs` was never wired —
  nothing in production deserialized a `Policy`, so no knob was reachable. The
  `sandbox` section of `settings.json` is the real surface.
- **Per-hostname allowlists require a proxy** (#477). Neither backend can filter
  egress by hostname: bwrap only toggles the netns, and Seatbelt's `(remote tcp
  …)` matches resolved socket addresses, not names. This is why #403 made bare
  domain lists fail closed. `http_proxy_port` is already honored by both
  backends; only the proxy process is missing. Until it lands, the opt-out is
  all-or-nothing.
- **An allowlist, when it lands, will bound *where* data can go, not *what*.**
  Allowing `github.com` so `gh pr create` works equally permits `gh gist
  create` — the credentials are *for* the allowed host. Claude Code documents
  the same weakness. This must be stated plainly rather than left for users to
  discover.
- **"Loopback" means different things on the two platforms, and macOS is
  weaker.** On Linux, `--unshare-net` gives the child its *own* network
  namespace: its loopback is private, and it cannot reach services listening on
  the host's `127.0.0.1`. On macOS, Seatbelt does not virtualize the network —
  allowing loopback allows the child to reach **the host's** loopback services.
  A hijacked command on macOS can therefore talk to anything you happen to be
  running locally (a database, an admin UI, a dev server) and — importantly — if
  you run a local forward proxy, it can reach the internet *through it*, routing
  around the egress block. Verified against real `sandbox-exec`. Closing this
  would require denying loopback on macOS, which breaks localhost test suites;
  we accept the residual risk and state it rather than pretend parity.
- **Environment scrubbing (#405)** remains open as follow-up defense-in-depth,
  now meaningful because egress is closed.
- **The Seatbelt generator emitted invalid SBPL in three places**, found by
  running the generated profile through the real `sandbox-exec` while
  implementing this: `(local ip "*:0")` for local binding and
  `(remote tcp "127.0.0.1:<port>")` for both proxy modes. Seatbelt requires the
  host to be literally `*` or `localhost` and the port to be a number or `*`;
  an invalid rule makes it reject the **entire profile**, so every sandboxed
  command fails to launch. These never fired only because no production policy
  set `allow_local_binding` or a proxy port — but the #406 fence sets the
  former, and #477 depends on the latter. Unit tests string-matched the
  generated text and so passed against an unusable profile; the fix adds tests
  that compile the profile with `sandbox-exec` itself.
- Depends on #476: bwrap skipped `--unshare-net` whenever `allow_local_binding`
  was set, so a local-sounding permission silently granted full egress. Fixed
  first; without it this decision would have been a no-op.
