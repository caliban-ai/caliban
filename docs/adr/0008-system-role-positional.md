# ADR 0008 · Role::System messages are positional (leading-only)

- **Status:** accepted
- **Date:** 2026-05-22

## Context

OpenAI's API treats system as a role: `system` messages can appear anywhere in the messages array. Anthropic's, Gemini's, and Bedrock-Claude's APIs treat the system prompt as a separate top-level field. Modeling both shapes uniformly in the IR was an open question.

## Decision

The IR has three roles: `User`, `Assistant`, `System`. System messages must appear contiguously at the start of `CompletionRequest.messages`. Validation rejects out-of-order System messages and System messages containing non-Text content blocks. Adapters with a separate-field system model (Anthropic, Gemini) collect the leading System messages and serialize them into the dedicated field; adapters with a system-role model (OpenAI, Ollama) pass them through as-is.

## Consequences

- **Positive:** Single canonical representation. Maps cleanly to all four families. Per-System-message `cache_control` (Anthropic feature) is preserved by serializing the system field as a block array when any block has a cache marker.
- **Negative:** Disallows the rare pattern of mid-conversation system injection. Callers wanting that pattern must rewrite into a "User says: here's a new constraint…" style.
- **Revisit if:** A provider semantically requires non-leading system messages, or a credible agent design needs mid-conversation system injection.
