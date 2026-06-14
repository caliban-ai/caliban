# ADR 0006 · Message schema → provider-neutral IR

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Layer 0 deferred the choice of message schema. Three approaches considered: (1) Anthropic-shape canonical; (2) provider-neutral IR; (3) lowest-common-denominator.

## Decision

Define caliban's own `Message`/`Content`/`StreamEvent` types (the IR) in `caliban-provider`. Each adapter translates `provider_native ↔ IR` at its boundary. The IR is intentionally close to Anthropic's API shape because Anthropic's API is the most expressive of the supported providers; other adapters lose less information when mapping to the IR.

## Consequences

- **Positive:** Adding a new provider doesn't touch `caliban-provider`. Provider-specific API changes don't ripple. The model-router (Layer 3) operates uniformly on IR. All transport variants of a given schema family share IR conversion code.
- **Negative:** One extra translation hop per request. IR design must capture the union of advanced features (thinking, prompt caching, multimodal) without becoming Anthropic-in-disguise.
- **Revisit if:** A provider emerges with feature semantics that can't be cleanly expressed in the IR (e.g., a new content modality the union doesn't anticipate).
