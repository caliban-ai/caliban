# ADR 0007 · Schema/transport factoring via Transport trait

- **Status:** accepted
- **Date:** 2026-05-22

## Context

A naïve "one crate per concrete provider endpoint" plan duplicates the Anthropic Claude schema work across `caliban-provider-anthropic` (direct API), an eventual Bedrock-Claude crate, and an eventual Vertex-Claude crate. Two orthogonal dimensions exist: model schema family vs. transport/endpoint.

## Decision

Each schema-family crate (`caliban-provider-anthropic`, `caliban-provider-openai`, `caliban-provider-google`, `caliban-provider-ollama`) defines its own `Transport` trait. A schema-family-generic `XxxProvider<T: Transport>` owns the IR conversion. Transport variants (DirectTransport, BedrockTransport, VertexTransport, AzureTransport, AIStudioTransport) are concrete `Transport` impls within their schema family, gated behind cargo features when they pull heavy deps (`aws-sdk-bedrockruntime`, `gcp_auth`).

## Consequences

- **Positive:** Claude-on-Bedrock and Claude-on-Vertex reuse the Anthropic IR-conversion code. Adding a new transport for an existing schema is a single-file change. The model-router can treat `(schema_family, transport)` as a tuple.
- **Negative:** A Transport trait is per-family, not shared across families — `caliban-provider-anthropic::Transport ≠ caliban-provider-openai::Transport`. This is intentional (transport contracts are not interchangeable across schemas).
- **Revisit if:** A transport pattern emerges that genuinely cross-cuts schema families (e.g., a future caliban-side mTLS proxy that wraps any provider).
