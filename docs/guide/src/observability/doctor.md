# Health Checks

`caliban doctor` runs a suite of local checks and reports whether your installation is healthy. It exits 0 when all checks pass or warn, and exits 1 if any check fails. The same checks are available as the `/doctor` slash command in the TUI.

## Running doctor

```bash
# Standard checks (no network calls)
caliban doctor

# Deep checks — pings every configured provider (costs one API call per provider)
caliban doctor --deep
```

Sample output:

```text
caliban doctor — 10 check(s):
  ✓ settings — 2 scope file(s) loaded
  ✓ sandbox — tool dispatch goes via caliban-sandbox::SandboxedShim
  ✓ checkpoint_store — /home/user/.local/share/caliban/projects
  ✓ session_store — /home/user/.local/share/caliban/sessions (writable)
  ✓ skills — 3 skill(s) loaded (scanned: /home/user/.claude/skills, ./.claude/skills)
  ✓ claudemd — 2 CLAUDE.md ancestor(s) found
  ✓ workspace — /home/user/repo (writable)
  ✓ openai — OPENAI_BASE_URL unset (no probe attempted; use --deep to ping api.openai.com)
  ✓ anthropic — https://api.anthropic.com reachable (45 model(s))
  ✓ google — GEMINI_BASE_URL unset (no probe attempted; use --deep to ping generativelanguage.googleapis.com)
```

## What each check covers

| Check | What it verifies |
|-------|-----------------|
| `settings` | Layered settings files load without parse errors; at least one scope file is present |
| `sandbox` | Tool dispatch is wired through the OS sandbox shim |
| `checkpoint_store` | The checkpoint store path is accessible |
| `session_store` | The session store path exists and is writable |
| `skills` | Skill roots are scanned and skills load without errors |
| `claudemd` | At least one `CLAUDE.md` file is found in the workspace ancestry |
| `workspace` | The current working directory is accessible and writable |
| `openai` | OpenAI / OpenAI-compatible endpoint reachability (incl. local `base_url` servers) |
| `anthropic` | Anthropic endpoint reachability |
| `google` | Google Gemini endpoint reachability |

## Provider reachability checks

Provider rows always appear in the output so you can see at a glance which providers are configured. The behavior depends on whether `--deep` is passed:

**Without `--deep`:**
- If the provider's base-URL env var is set, caliban probes the endpoint (`/v1/models`).
- If the env var is unset, the row passes with a note that no probe was attempted. Use `--deep` to ping the default endpoint.

**With `--deep`:**
- Caliban pings the configured (or default) endpoint unconditionally. This costs one real API call per provider that has an API key configured.
- If `--model <MODEL>` was passed on the same invocation and a provider's model listing is available, the requested model is verified to be present. A missing model is reported as a `Fail` row.

```admonish note title="Local endpoints without an API key"
A local OpenAI-compatible server needs no API key ([ADR 0056](../adr/README.md)). Set `OPENAI_BASE_URL` to your endpoint; with `--deep`, caliban probes it regardless of key configuration.
```

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed or warned |
| 1 | At least one check failed |

CI scripts can gate on `caliban doctor` to catch misconfigured installations before running a long job:

```bash
caliban doctor || { echo "caliban health check failed"; exit 1; }
```

## `/doctor` in the TUI

The `/doctor` slash command runs the same checks inside an interactive session and prints the results to the transcript. As on the CLI, provider pings are deep only when you pass `--deep` (`/doctor --deep`). The `/status` command shows the active provider name.

## Related pages

- [Telemetry & Cost](./telemetry.md) — OTLP export and cost accounting
- [Headless & Audit](../permissions/headless-and-audit.md) — permission auditing in CI
