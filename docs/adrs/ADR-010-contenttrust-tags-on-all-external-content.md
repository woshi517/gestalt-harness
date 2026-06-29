# ADR-010: ContentTrust Tags on All External Content

**Status:** Accepted

## Context
External content (web, PDF, MCP, retrieved chunks) can contain adversarial prompt injection. The context pipeline must treat it differently from user-authored instructions.

## Decision
All content items carry `ContentTrust`. `TrustBoundaryRenderer` wraps untrusted items in explicit markup before rendering to the provider.

## Consequences
Prompt injection from external sources is structurally harder. The model receives a clear signal about the provenance of each content block. Legitimate use cases (reading a web page) are unaffected — only the trust markup is added.
