# Philosophy

Caliban exists because the dominant AI agent CLIs are tightly coupled to a single provider
and leave operators with little control over what the model sees, what it can do, or where
state is stored. The design is a direct response to those constraints.

## Operator control

You decide what model handles each task, what context goes into the prompt, and which tools
the model is allowed to call. Routing is declarative (`caliban.toml`); settings layer at
four scopes (managed, user, project, local) with deep-merge semantics; permissions are
first-class and auditable. Nothing is hardwired to a service the operator does not control.

## Provider-agnostic

No SDK lock-in. Anthropic Claude, OpenAI, Google Gemini, and local Ollama all speak the same
internal representation inside Caliban. Cloud transports (AWS Bedrock, Google Vertex, Azure
OpenAI) are cargo-feature-gated and additive — the core binary has no mandatory cloud
dependency. Switching providers is a flag, not a rewrite.

## Local-first and data sovereignty

Sessions, checkpoints, auto-memory, and tool-result overflows live on your disk by default.
Caliban is designed to run in a self-hosted homelab: no required cloud account, no telemetry
unless you opt in (`CALIBAN_ENABLE_TELEMETRY=1`), no state sent anywhere you do not control.

## AGPL-3.0 transparency

Caliban is licensed under AGPL-3.0-only. If you modify Caliban and run it as a network
service or distribute the binary, you must release your changes under the same license. This
closes the "SaaS loophole" that GPL-3.0 leaves open, aligning with projects like Mastodon
and Nextcloud that use AGPL to keep improvements in the commons. Personal use is unaffected.
The full rationale is in [ADR 0003](../appendix/adrs.md).

## Rust performance

Harness overhead should be negligible compared to model latency. The time-to-result you
experience is dominated by the model, not the runtime. This is not a feature worth advertising
loudly — it is a baseline expectation for a tool that runs constantly in the background.

```admonish note title="What Caliban does not try to be"
Caliban is a terminal agent harness, not an IDE extension, a cloud service, or a mobile app.
IDE integration, GitHub App, and remote-control surfaces are tracked in the parity matrix
(theme N) but are explicitly parked until the terminal/CLI feature set reaches parity with
Claude Code. The guide does not document planned features as if they were shipped.
```
